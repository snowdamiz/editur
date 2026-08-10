# Claude ACP integration plan

## 1. Purpose and reader

This is the Claude-specific follow-on to the [multi-provider ACP refactor plan](ACP_PROVIDER_PLAN.md) and the sibling [Codex ACP integration plan](CODEX_ACP_PLAN.md). It is for an Editur contributor taking the existing disabled Claude catalog entry to a releasable third provider.

After reading this plan, the contributor should be able to pin, package, launch, authenticate, exercise, switch away from, repair, and release Claude through the canonical ACP adapter without adding a second controller, collecting credentials, offering prohibited subscription login, or weakening Editur's supply-chain rules.

Existing code may already satisfy some items. Treat it as a candidate implementation: prove each exit condition with a focused test or smoke result before checking it off.

## 2. Definition of done

The Claude integration is complete only when:

- A release build containing Cursor, Codex, and Claude shows all three in the provider selector in both Agent layouts.
- Selecting Claude displays the adapter and Anthropic terms notice, provisions only the pinned private package, and starts it without `npx`, a global Node.js installation, or a mutable package tag.
- The managed launch uses the adapter's `--hide-claude-auth` mode, removes `CLAUDE_CODE_OAUTH_TOKEN`, and cannot be redirected through `CLAUDE_CODE_EXECUTABLE`, `NODE_OPTIONS`, `NODE_PATH`, or another executable override.
- Claude works with an environment-provided Anthropic API key, supported commercial cloud credentials, or provider-owned Console credentials. Editur never asks for, receives, logs, or persists a credential.
- Editur never advertises Claude.ai Free, Pro, or Max subscription login unless Anthropic gives prior written approval and a later plan explicitly enables it.
- Standard ACP sessions, streaming, tools, plans, permissions, cancellation, images, usage, model/mode/configuration controls, commands, and history render through the shared controller and state reducer.
- Claude-specific nested transcripts, steering, goals, gateway configuration, and other namespaced extensions stay disabled until a separate fixture-backed slice justifies them.
- Switching to or from Claude stops the old process tree first and never carries a session id, permission, interaction, transcript, capability, or diagnostic across providers.
- All release targets pass package verification and process teardown checks.
- Legal/distribution review and a real commercial-account smoke run are recorded before stable promotion.

## 3. Scope

### 3.1 Included

- Canonical `@agentclientprotocol/claude-agent-acp` integration over stdio ACP.
- A pinned adapter, its exact locked Claude Agent SDK dependency, the matching platform-native Claude binary, and a private Node.js runtime.
- Cursor/Codex-to-Claude and Claude-to-Cursor/Codex selection, persistence, and switching.
- Environment-owned `ANTHROPIC_API_KEY` authentication and provider-owned commercial credentials supported by the pinned SDK.
- Existing supported commercial backends, such as Anthropic API, Amazon Bedrock, Claude Platform on AWS, Google Vertex AI, and Microsoft Azure, when configured outside Editur through the SDK's documented environment and credential mechanisms.
- Capability-driven session history, model, mode, fast-mode, tool, plan, permission, image, edit-review, command, usage, and MCP presentation where these arrive through standard ACP.
- Provider-scoped installation, verification, cache, diagnostics, and hidden-session state.
- Native macOS Apple Silicon, macOS Intel, Linux x86_64, and Windows x86_64 release artifacts.

### 3.2 Excluded

- Claude.ai Free, Pro, or Max login and subscription-rate-limit use through Editur.
- An Editur API-key form, credential vault, cloud-provider configurator, gateway header editor, or environment editor.
- Global Node.js installation, `npx`, remote install scripts, or `PATH` mutation.
- User-selectable adapter implementations under the Claude provider id.
- Claude-only transcript rendering, a second controller, or a second permission system.
- Terminal authentication until Editur has a real, bounded terminal-auth surface and Anthropic approves the login method for this product.
- `_meta.claudeCode.*`, nested subagent transcripts, steering, goals, configurable providers, gateway authentication, or elicitation UI in the first release.
- Simultaneous providers, background provider processes, or shared sessions between providers.

## 4. Prerequisite and current snapshot

The shared work in [ACP_PROVIDER_PLAN.md](ACP_PROVIDER_PLAN.md) and the proven Codex path in [CODEX_ACP_PLAN.md](CODEX_ACP_PLAN.md) are prerequisites: closed provider catalog, generic controller, format-v2 provider bundle, provider-scoped storage, safe switching, capability-driven UI, deterministic private-runtime packaging, and provider-gated extensions.

Snapshot checked 2026-08-09:

| Concern | Candidate state | Required audit |
| --- | --- | --- |
| Provider catalog | `claude` id and metadata exist but are marked unavailable | Replace the provisional terms link, retain `ProviderExtensions::None`, and enable only when a Claude manifest ships |
| Provisioning | Claude policy, host validation, and download validation intentionally reject every package | Add only the exact compiled policy for the audited release |
| Adapter | `@agentclientprotocol/claude-agent-acp` `0.66.0` | Pin commit and npm integrity; verify ACP behavior from a sanitized fixture |
| SDK dependency | `@anthropic-ai/claude-agent-sdk` `0.3.220` from the adapter lockfile | Verify package integrity, included legal notices, and the matching target-native optional package |
| Runtime | Codex already packages private Node.js `22.22.0`; the Claude adapter requires Node.js 22 or newer | Reuse the audited runtime inputs in CI but keep Claude's installed tree provider-scoped |
| Authentication | Shared UI preserves auth kinds but does not advertise terminal auth; no Claude environment fallback exists | Enforce commercial/API mode and provide non-secret setup guidance when credentials are missing |
| Controller | Standard ACP is shared; one Codex-specific auth normalization remains in the controller | Move provider auth normalization to the provider boundary before adding Claude behavior |
| UI and switching | Generic selector, terms, state reset, persistence, and ordered shutdown are present | Prove the same behavior with three packaged providers |
| Release CI | Cursor and Codex packages and rollback bundles are produced | Add an independently verified, lazy Claude archive and a bundle-without-Claude rollback candidate |

Do not preserve a candidate behavior merely because it exists. The acceptance criteria in this document win.

## 5. Pinned integration decision

### 5.1 Adapter

Use the canonical [`agentclientprotocol/claude-agent-acp`](https://github.com/agentclientprotocol/claude-agent-acp) adapter. The initial audited pin is:

- Package: `@agentclientprotocol/claude-agent-acp`
- Version: `0.66.0`
- Commit: [`6b405138fc82be947964612fac04e56654827b66`](https://github.com/agentclientprotocol/claude-agent-acp/commit/6b405138fc82be947964612fac04e56654827b66)
- Adapter source license: Apache-2.0
- Locked SDK: `@anthropic-ai/claude-agent-sdk` `0.3.220`
- Required adapter runtime: Node.js 22 or newer

This is an ACP ecosystem adapter built on Anthropic's official Claude Agent SDK. It is not a native ACP server maintained solely by Anthropic.

The adapter source is Apache-2.0, but the locked Claude Agent SDK and native binary are proprietary and governed by Anthropic's legal agreements. The ACP Registry therefore classifies the complete Claude agent distribution as proprietary. User-facing terms and release review must describe the complete bundle, not imply that Apache-2.0 covers the SDK or native binary.

Do not silently replace the canonical adapter with a fork or direct SDK integration. A replacement requires a new compatibility, licensing, authentication, and distribution review against this plan.

### 5.2 Authentication and branding decision

Anthropic's current Agent SDK terms direct third-party products to API-key or supported cloud-provider authentication and prohibit offering Claude.ai login or routing Free, Pro, or Max credentials without prior approval. Therefore the initial Editur release must:

- Pass `--hide-claude-auth` to the adapter.
- Remove `CLAUDE_CODE_OAUTH_TOKEN` from the child environment.
- Not advertise terminal-auth or gateway-auth client capabilities.
- Label the provider `Claude` or `Claude Agent`, never `Claude Code` or `Claude Code Agent` in user-facing UI.
- Link the adapter license, Anthropic Commercial Terms, legal/compliance guidance, data-usage policy, and privacy policy from the first-use notice or adjacent documentation.

If Anthropic later approves subscription login for Editur, treat that as a new product and legal slice. Do not unlock it by removing one argument.

### 5.3 Runtime distribution

For the first release, build the exact adapter commit with its committed lockfile and package it with:

- The built adapter entrypoint, package metadata, README, and Apache-2.0 license.
- The exact production dependency tree selected by the lockfile.
- `@anthropic-ai/claude-agent-sdk` `0.3.220`, its license/readme notices, and only the native optional package matching the release target.
- A private, exact Node.js `22.22.0` runtime and its license.

Reuse the Codex CI machinery for fetching and verifying Node.js. Keep the Claude archive and installed root independent; a shared runtime installation is deferred until measurements prove duplicate optional downloads are a real problem.

Do not package every platform-native SDK binary into every target. Do not depend on a global Claude installation or `CLAUDE_CODE_EXECUTABLE`.

### 5.4 Upgrade rule

An upgrade changes the adapter version, commit, SDK dependency, native binary package, Node runtime, or package layout only through one reviewed change. It must regenerate every target artifact, refresh checksums and legal notices, rerun the automated and manual matrix, and document capability, authentication, data-handling, or terms differences.

No runtime component may update itself inside Editur's managed directory.

## 6. Architecture and launch contract

```text
Provider selector
      |
      v
shared AgentController + ProviderId::Claude
      |
      v
prepared managed launch
private Node -> claude-agent-acp -> Claude Agent SDK -> bundled native Claude binary
      |
      v
standard ACP commands, events, permissions, and session capabilities
```

The provider layer owns the exact executable, entrypoint, arguments, environment policy, package verification, display metadata, authentication setup guidance, and terms policy. The shared controller owns ACP initialization, sessions, prompts, cancellation, permissions, normalized events, and shutdown.

The prepared Claude launch must:

- Execute the managed private Node binary by absolute path.
- Pass the managed adapter entrypoint by absolute path.
- Pass exactly `--hide-claude-auth` for the initial release.
- Use the selected project root as the working directory.
- Use a Claude-provider cache directory for Node's compile cache.
- Resolve the native Claude binary only from the bundled target-specific SDK optional dependency.
- Remove `CLAUDE_CODE_EXECUTABLE`, `CLAUDE_CODE_OAUTH_TOKEN`, `NODE_OPTIONS`, and `NODE_PATH` even when the parent environment sets them.
- Inherit approved user-owned commercial auth, backend, proxy, and trusted-CA inputs without copying their values into Editur state or logs.
- Preserve the user's Claude configuration behavior so the SDK can load user, project, local, and managed settings. Editur must not read provider credential files or duplicate provider settings.
- Make no global environment or filesystem changes.

The launcher must contain the entire descendant tree. Closing Agent, switching provider, application exit, initialization failure, or a broken stdout stream must terminate the adapter and native Claude child on every target.

## 7. Authentication contract

### 7.1 Anthropic API key

- `ANTHROPIC_API_KEY` remains owned by the user's environment or secret manager.
- Editur never renders a secret field, reads the value, copies it into preferences, or sends it through an ACP request.
- When Claude reports authentication required and offers no supported method, show environment setup guidance naming `ANTHROPIC_API_KEY`, then offer Retry and provider switching.
- Do not turn the environment setup row into an Authenticate button; restarting or relaunching the provider after the environment changes is the completion path.

### 7.2 Commercial cloud credentials

- Preserve the SDK's documented environment-driven Anthropic API, Bedrock, Claude Platform on AWS, Vertex AI, and Azure AI Foundry selection and credential chains.
- Do not create a cloud-provider selector or credential editor in Editur.
- Show only generic documentation guidance unless the pinned adapter advertises safe, non-secret standard ACP metadata.
- Test at least direct Anthropic API authentication before release. Treat cloud backends as supported passthrough only after their native smoke result is recorded.

### 7.3 Provider-owned Console credentials

The SDK may reuse a commercial credential already stored by its native tooling. Editur may rely on that provider-owned store but must not inspect, migrate, export, or delete it. Logout remains hidden until the shared client implements and tests stable ACP logout without exposing provider data.

### 7.4 Unsupported authentication

- Do not advertise ACP terminal-auth capability in the initial release.
- Do not advertise the adapter's gateway-auth capability or call its configurable-provider methods.
- Never offer `claude-ai-login`, accept `CLAUDE_CODE_OAUTH_TOKEN`, or reuse Free, Pro, or Max credentials.
- Keep `--hide-claude-auth` in the compiled manifest policy so downloaded metadata cannot remove it.
- If the adapter still exposes subscription login or accepts a subscription under this contract, block the release.

### 7.5 Provider-owned auth normalization

Move the existing Codex API-key method normalization out of the shared controller into the provider boundary, then add Claude's environment-only fallback there. The controller should consume normalized choices without matching on Codex or Claude.

The Claude fallback exists only when the pinned adapter returns no usable methods and session creation reports authentication required. It names credential variables, never values, and cannot send an ACP `authenticate` request.

### 7.6 Credential and privacy boundary

Editur must not read Claude auth files, echo auth or cloud-provider environment values, include secrets in diagnostics, or copy provider logs into the transcript. Raw Claude stderr is bounded and suppressed from normal UI; a future diagnostic export must be opt-in, redacted, provider-scoped, and reviewed.

## 8. ACP capability mapping

Start from standard ACP. The pinned adapter provides images, embedded context, HTTP/SSE MCP, session list/load/resume/fork/close/delete, model and mode configuration, fast mode, plans, tools, edit review, permissions, usage, available commands, and session titles through standard capabilities, responses, or updates.

For each initialization or session response:

- Show History only when list and load are both advertised and verified for the selected project.
- Populate model, mode, fast-mode, agent, and other controls only from session config options.
- Preserve the exact permission choices and outcomes supplied by the adapter.
- Normalize streamed text, thought, plan, tool, diff, image, ordinary Bash/tool output, usage, command, title, and completion events through existing event types.
- Keep Cursor's `run-everything` behavior hidden for Claude.
- Ignore unknown namespaced metadata safely.

Do not advertise or implement the first-release extensions for nested subagent transcripts, `_session/steering`, session goals, configurable LLM providers, gateway authentication, specialized terminal output, or elicitation. The adapter's protocol-compatible flattened and ordinary tool-result fallbacks remain the baseline. If disabling an extension removes a required core workflow, record a sanitized fixture and add the smallest shared or Claude-gated slice separately.

Capture a sanitized initialization/session fixture from the exact adapter and SDK pin. It must contain no account ids, provider headers, project paths outside a temporary directory, prompt content, credentials, or raw provider logs.

## 9. Session, configuration, and switching behavior

- Claude sessions remain SDK-owned; Editur persists only the selected provider and its own hidden-session removals.
- Hidden-session state is namespaced by Claude and project.
- A Claude session id is never loaded through Cursor or Codex or copied into another provider state.
- Provider-advertised models, modes, config options, auth status, usage, commands, titles, and diagnostics are live state and are cleared on switch.
- The unsent composer draft survives switching but is never submitted automatically.
- A non-empty transcript requires confirmation before leaving Claude.
- Switching is disabled during a turn, permission request, or interaction request.
- The UI stays in `Switching to ...` until the entire old process tree is gone.
- A failed Claude installation, authentication, or connection keeps Claude selected so Retry and setup guidance remain meaningful.
- Switching back to Cursor or Codex follows the same lifecycle and never revives an old process.

Use the adapter's standard list/load behavior only after the pinned fixture and real smoke run prove project scoping, title replay, and session continuation. Otherwise hide History and create a new session; do not emulate history from Editur transcripts.

## 10. Provisioning and supply-chain contract

### 10.1 Build inputs

CI must fetch only:

- Adapter commit `6b405138fc82be947964612fac04e56654827b66`.
- Exact npm packages and integrities resolved by its committed lockfile.
- The one platform-native Claude Agent SDK optional package matching the release target.
- The exact Node.js `22.22.0` target archive verified against the official checksum list.

Build with lifecycle scripts disabled unless a reviewed dependency requires one specific audited script. Do not resolve `latest`, a semver range without the lockfile, an unpinned Git ref, or a host-global module.

### 10.2 Deterministic package

The target archive must have stable ordering, timestamps, path separators, and executable modes. Reject symlinks, path traversal, duplicate paths, special files, undeclared content, non-target native binaries, and an absent native binary.

The generated manifest pins:

- Provider id, adapter version, SDK version, and target-native package identity.
- Target operating system and architecture.
- Exact archive URL, size, checksum, and format.
- Exact managed Node command, adapter entrypoint, and `--hide-claude-auth` argument.
- Exact extracted files, kinds, sizes, checksums, and executable flags.
- Compression, extraction, and entry-count limits.
- The approved adapter-license and Anthropic-terms URLs. SDK legal and attribution notices remain declared files inside the archive.
- Version probes and expected output.

### 10.3 Runtime verification

Before embedding a provider bundle, CI must extract the finished archive through the application provisioner and prove:

1. The adapter `--version` probe returns `0.66.0` from the managed tree.
2. The installed SDK package metadata and integrity identify `@anthropic-ai/claude-agent-sdk` `0.3.220`.
3. The target-native Claude binary resolves from the declared optional package and passes its pinned version probe.
4. A no-network launch fixture proves the private Node runtime can load the adapter and locate that native binary.

Reject a package if any probe resolves through host `PATH`, a global module directory, `CLAUDE_CODE_EXECUTABLE`, or an undeclared file.

### 10.4 Distribution and updates

- Publish Claude archives under Editur's pinned provider release namespace.
- Allow only the exact GitHub release host and documented release-asset redirect host.
- Attest the archive and provider bundle produced by the native release job.
- Keep Cursor in every format-v2 bundle; Codex and Claude remain optional and lazy.
- Bootstrap installers continue provisioning Cursor only.
- Self-update validates the new bundle and repairs Cursor; Claude repairs lazily on next selection.
- Removing Claude from a future bundle hides it without recursively deleting its managed files, sessions, settings, or credentials.
- Keep a tested release candidate containing Cursor and Codex but no Claude.

## 11. UI contract

When the embedded bundle contains Cursor, Codex, and Claude:

- The IDE Agent header begins with `[Claude ⌄]` when Claude is selected.
- The full Agent session rail uses the same provider identity control.
- The provider menu shows `Claude`, the existing catalog icon, description, selected check, and `Installs on first use` status.
- Model, mode, fast-mode, agent, and other configuration selectors remain in the composer and update only after the Claude session is ready.
- First selection opens a terms notice that distinguishes the Apache-2.0 adapter from the proprietary SDK/native binary and links the applicable Anthropic terms.
- Provisioning reads `Installing Claude Agent`; connection and failure copy use Claude metadata.
- Missing credentials show API/environment setup guidance, Retry, and provider switching. They never show a subscription-login or secret-entry action.
- Unsupported extension controls disappear rather than appearing as disabled placeholders.
- Disabled provider rows explain that the user must stop or answer the active request first.
- All rows remain keyboard reachable and expose a complete accessibility label.

A build without a Claude manifest must keep the catalog entry unavailable and must not offer a selectable Claude row.

## 12. TDD and verification strategy

Use one meaningful red-green cycle per non-trivial behavior. Reuse the fake ACP process, Codex package pipeline, and generic state/UI tests; do not clone the controller suite for Claude.

### 12.1 Provider and launch tests

- Claude catalog metadata uses the audited terms, stays lazy, and has no vendor extension enabled.
- A prepared Claude launch uses only managed absolute command and entrypoint paths and includes exactly `--hide-claude-auth`.
- The launch removes `CLAUDE_CODE_EXECUTABLE`, `CLAUDE_CODE_OAUTH_TOKEN`, `NODE_OPTIONS`, and `NODE_PATH` even when the parent environment sets them.
- Approved credential variable names may reach the child while values never appear in events, errors, receipts, diagnostics, or snapshots.
- Claude uses its provider-scoped cache and managed root.
- Cursor, Codex, and Claude paths, locks, receipts, and active markers cannot collide.

### 12.2 Manifest and package tests

- The exact release specification generates a valid Claude manifest for every target.
- Wrong host, redirect, argument, command, entrypoint, platform, checksum, file set, native package, executable bit, terms URL, or version response is rejected.
- Deterministic packaging produces identical bytes from identical staged content.
- Archive extraction rejects traversal, links, special files, extra entries, oversized data, missing files, and non-target native packages.
- Adapter, SDK, and native binary probes resolve from the extracted managed tree.
- A corrupt Claude installation repairs without changing Cursor or Codex.

### 12.3 ACP controller tests

- The same generic prompt, stream, tool, permission, cancellation, history, and shutdown flow passes for Cursor, Codex, and Claude provider ids.
- The sanitized Claude initialization fixture produces the expected history, attachment, command, and configuration capabilities.
- Cursor extensions are ignored for Claude.
- Unknown Claude metadata is ignored without disconnecting.
- Nested subagents remain usable through the flattened fallback without advertising the nested-transcript capability.
- Missing credentials produce environment setup guidance, not a blank failure or Authenticate request.
- Subscription auth, terminal auth, gateway auth, and provider-configuration UI remain unavailable.
- A sanitized auth fixture reporting a subscription account is rejected while `--hide-claude-auth` is active.
- Malformed stdout, bounded stderr, adapter exit, and native child exit become provider-aware connection failures without exposing raw provider output.

### 12.4 State and UI tests

- The selector lists all three exact catalog entries when all three manifests are packaged.
- Claude identity, description, lazy-install status, terms, and connection copy are visible and unclipped.
- No user-facing string labels the provider `Claude Code`.
- Provider selection remains distinct from model and mode selection.
- Switching confirmation, blocking, draft preservation, state clearing, and shutdown ordering are enforced for every direction involving Claude.
- History and configuration controls follow advertised capabilities.
- A failed Claude start remains selected and exposes setup guidance, Retry, and provider switching.

### 12.5 Process tests

- Unix shutdown leaves no adapter or native Claude descendant.
- Windows assigns the complete tree to one kill-on-close job object.
- Closing stdin, cancelling initialization, switching, and application exit converge on the same teardown result.
- A hung child is bounded by the existing shutdown timeout and cannot keep Editur alive.

## 13. Manual smoke matrix

Run with a disposable commercial Anthropic API key or approved commercial cloud account. CI must not receive real credentials or spend paid tokens.

For each supported target:

1. Start from a clean Editur data directory with Cursor and Codex available.
2. Open Agent and verify no Claude work happened before this point.
3. Select Claude, review the complete terms notice, provision, and record archive and installed sizes.
4. Start without credentials and verify API/environment setup guidance, no subscription-login action, and no secret field.
5. Relaunch with `ANTHROPIC_API_KEY`; confirm the key never appears in UI, logs, diagnostics, or persisted files.
6. Record protocol version, auth methods, capabilities, models, modes, config options, available commands, and session features.
7. Create a session, stream a response, and send a same-session follow-up.
8. Exercise a plan, tool, file edit, exact allow/reject permission, cancellation, image input, edit review, and MCP when advertised.
9. Verify buffer reconciliation and project refresh.
10. List/load history and resume a session when advertised; otherwise verify History is absent.
11. Exercise a subagent and verify the non-extension flattened fallback remains coherent.
12. Switch to Cursor and Codex, confirm Claude descendants exit, switch back, and verify provider/session isolation.
13. Corrupt one managed Claude file and verify lazy repair.
14. Exit Editur and confirm no adapter or native Claude descendant remains.

Record time to first connection, time to warm connection, archive size, installed size, native binary identity, unsupported features, commercial backend, and adapter diagnostics. Redact account, project, prompt, path, and credential data.

## 14. Implementation phases

### Phase 0: Close legal and distribution gates

- Confirm the adapter, SDK, native binary, branding, redistribution, commercial terms, data handling, and first-use disclosure with the release owner.
- Confirm that `--hide-claude-auth` and removal of `CLAUDE_CODE_OAUTH_TOKEN` satisfy the no-subscription requirement.
- Record the exact adapter commit, npm integrities, SDK/native package matrix, and Node checksums.

Exit condition: Editur has a documented permitted distribution and API/commercial authentication path. Otherwise Claude remains unavailable and no UI or packaging work proceeds.

### Phase 1: Audit and lock the managed launch

- Trace the existing disabled Claude entry through catalog availability, manifest policy, package preparation, controller start, and teardown.
- Add failing tests for Claude's exact arguments and executable/preload overrides.
- Add the smallest Claude policy to the existing provider release and provisioning machinery.
- Prove absolute managed Node, entrypoint, target-native binary, cache, and descendant containment behavior.

Exit condition: a hostile parent environment cannot redirect or preload the managed Claude runtime, and subscription credentials cannot enable the prohibited auth path.

### Phase 2: Close authentication behavior

- Move provider auth normalization out of the shared controller.
- Preserve Codex behavior through focused regression tests.
- Add Claude's environment-only setup fallback and ensure it cannot send an ACP Authenticate request.
- Keep terminal, subscription, gateway, and configurable-provider capabilities unadvertised.
- Suppress raw Claude diagnostics from normal UI.

Exit condition: every visible Claude authentication state has a tested recovery path, no credential crosses Editur state, and no consumer subscription path is reachable.

### Phase 3: Prove standard ACP compatibility

- Add the sanitized pinned-adapter fixture.
- Run shared command/event/state behavior for Claude.
- Verify capability-driven history, attachments, commands, and configuration controls.
- Prove tool, permission, edit, usage, cancellation, and flattened-subagent fallbacks.
- Keep namespaced Claude metadata ignored.

Exit condition: Claude requires no transcript fork, controller fork, or vendor extension for the release baseline.

### Phase 4: Harden packaging and release

- Build the exact adapter and production dependency tree on each native release runner with lifecycle scripts disabled.
- Stage only the matching SDK native package and the audited private Node runtime.
- Run deterministic archive, manifest, extraction, adapter-version, SDK-version, native-version, and no-global-runtime probes.
- Add Claude to the format-v2 provider bundle and produce a bundle-without-Claude rollback candidate.
- Publish and attest provider archives with the application artifacts.

Exit condition: every native release binary contains an attested bundle that can lazily install its matching Claude archive.

### Phase 5: Complete UI and switching

- Remove the catalog's unavailable reason only when the matching manifest is embedded.
- Verify the provider selector, complete terms, API setup guidance, capability controls, failure states, branding, and accessibility in both layouts.
- Prove old-process shutdown precedes new-process launch.
- Prove state, sessions, diagnostics, and managed files remain isolated across all three providers.

Exit condition: a user with commercial Claude credentials can move among Cursor, Codex, and Claude without process, state, session, or credential leakage.

### Phase 6: Release gate

- Run the full automated suite and native packaging jobs.
- Complete and record the real commercial-account smoke matrix.
- Recheck adapter/SDK terms, branding, dependency lock, archive checksums, auth behavior, data handling, and update behavior.
- Exercise rollback with a release candidate that omits Claude.

Exit condition: Claude can be promoted or removed without disabling Cursor or Codex or corrupting provider data.

## 15. Acceptance criteria

- [ ] Claude uses the canonical pinned adapter, exact locked SDK, and matching target-native binary.
- [ ] The private runtime and every managed file are pinned, verified, licensed, and attested.
- [ ] Editur never invokes `npx`, a global Node or Claude installation, a mutable tag, or a remote install script.
- [ ] `--hide-claude-auth` is fixed by compiled release policy.
- [ ] `CLAUDE_CODE_EXECUTABLE`, `CLAUDE_CODE_OAUTH_TOKEN`, `NODE_OPTIONS`, `NODE_PATH`, and equivalent execution overrides cannot redirect or weaken the managed launch.
- [ ] Editur does not offer Claude.ai Free, Pro, or Max login or use subscription credentials.
- [ ] Anthropic API and commercial cloud credentials remain environment/provider-owned; no secret enters Editur UI, state, logs, diagnostics, or persistence.
- [ ] No Claude provisioning, installation inspection, process, or repaint loop starts before Agent is opened.
- [ ] Standard ACP controller, events, transcript state, and buffer reconciliation remain shared.
- [ ] Claude has no vendor extension enabled for the first release.
- [ ] History, attachments, commands, configuration, model, mode, fast-mode, and permission UI follow advertised capabilities.
- [ ] The three-provider build shows Claude in both layouts; a bundle without Claude does not offer it.
- [ ] User-facing branding says `Claude` or `Claude Agent`, not `Claude Code`.
- [ ] Terms acceptance precedes the first Claude download and distinguishes adapter and SDK terms.
- [ ] Claude installation, cache, receipts, hidden sessions, configuration, and diagnostics are provider-scoped.
- [ ] Switching is blocked during active requests, stops the old tree first, preserves only the draft, and clears provider-bound state.
- [ ] A failed Claude start stays selected and can Retry or switch back.
- [ ] Automated tests require no account, networked prompt, or paid token.
- [ ] Every supported target passes package probes, native teardown, and the manual provider smoke matrix.
- [ ] A bundle without Claude remains functional and leaves Claude-owned data untouched.

## 16. Risks and stop conditions

| Risk | Default response | Revisit when |
| --- | --- | --- |
| Anthropic terms do not permit Editur's redistribution or product use | Keep Claude unavailable | Written review confirms the exact adapter/SDK distribution |
| Subscription login remains reachable | Block Claude | `--hide-claude-auth`, environment filtering, and the sanitized subscription fixture prove it is unreachable |
| Authentication requires Editur to collect a secret | Block that method | Environment, provider-owned storage, or approved workload identity completes it safely |
| SDK/native dependency is not exactly reproducible | Block the release | Exact lockfile, integrity, target package, notices, and artifacts reproduce on every target |
| Managed launch honors an executable or preload override | Block the release | The override is removed and covered by a child-environment test |
| Adapter needs global Node, Claude, `npx`, or a mutable download | Block Claude | A pinned private runtime passes the full package contract |
| Native binary self-updates managed files | Block Claude | A supported immutable/update-disabled path exists |
| Native package does not match the release target | Block that target | The extracted native version probe passes on that target |
| Process teardown leaves a descendant | Block that target | Native containment and exit checks pass repeatedly |
| Session list/load is unreliable | Hide History | The adapter passes stable project-scoped list/load and resume behavior |
| A namespaced extension appears useful | Ignore it | A fixture proves standard ACP loses material information and a provider-gated slice is reviewed |
| Claude archive duplicates too much Node data | Keep it lazy | Measurements justify a secure shared-runtime design |
| Terms, branding, auth, or data handling changes | Remove Claude from the bundle | Legal and product review approves the new contract |

## 17. Reference snapshot

Checked 2026-08-09:

- [Multi-provider ACP refactor plan](ACP_PROVIDER_PLAN.md)
- [Codex ACP integration plan](CODEX_ACP_PLAN.md)
- [Claude ACP Registry entry](https://github.com/agentclientprotocol/registry/blob/main/claude-acp/agent.json)
- [Canonical Claude ACP adapter](https://github.com/agentclientprotocol/claude-agent-acp)
- [Pinned adapter release](https://github.com/agentclientprotocol/claude-agent-acp/releases/tag/v0.66.0)
- [Pinned adapter commit](https://github.com/agentclientprotocol/claude-agent-acp/commit/6b405138fc82be947964612fac04e56654827b66)
- [Claude Agent SDK overview and branding guidance](https://code.claude.com/docs/en/agent-sdk/overview)
- [Anthropic legal and credential-use guidance](https://code.claude.com/docs/en/legal-and-compliance)
- [Claude API authentication](https://platform.claude.com/docs/en/manage-claude/authentication)
- [Claude data-usage policy](https://code.claude.com/docs/en/data-usage)
- [Anthropic Commercial Terms](https://www.anthropic.com/legal/commercial-terms)
- [Anthropic Privacy Policy](https://www.anthropic.com/legal/privacy)
- [Agent Client Protocol](https://agentclientprotocol.com/)

Versions, capabilities, auth methods, package layout, terms, and data handling are release inputs. Re-pin and re-verify them for every Claude upgrade; never discover or adopt them dynamically at runtime.
