# Multi-provider ACP refactor plan

## 1. Purpose

Refactor Editur's working Cursor integration so a second ACP agent can be added without duplicating the controller, transcript state, permission flow, buffer reconciliation, or Agent UI.

This plan is for the engineer implementing the next stage. After reading it, they should be able to preserve Cursor behavior, establish the smallest useful provider boundary, add Codex as the proof provider, and then add Claude through the same path.

The Codex proof-provider work is specified separately in [CODEX_ACP_PLAN.md](CODEX_ACP_PLAN.md).

In this document, **provider** means an ACP-compatible agent process such as Cursor, Codex, or Claude. It does not mean the model selected inside that agent.

## 2. Product contract

The refactor and first multi-provider release must:

- Keep Cursor as the default and preserve its current installation, authentication, sessions, extensions, and UI behavior.
- Keep the existing normalized ACP command, event, transcript, tool, permission, and interaction model shared by every provider.
- Run only one provider process and one active turn per Editur window.
- Start no provider process, provisioning work, registry request, or repaint loop until the Agent view is opened.
- Let the user choose a provider from the Agent header once at least two providers are actually available.
- Keep provider selection separate from model, mode, sandbox, and permission controls advertised by the active ACP session.
- Scope sessions, hidden-session history, authentication status, configuration, diagnostics, and vendor extensions to the selected provider.
- Shut down the current provider before starting another one and never carry a session identifier or permission request across providers.
- Pin and verify every managed provider release. Do not run a mutable `latest` package or trust a download host merely because it appears in downloaded JSON.
- Degrade by capability: hide unsupported history, configuration, mode, or extension UI instead of emulating it.

The first multi-provider release will not include:

- Simultaneous providers, parallel conversations, or background agents.
- A public plugin API, dynamic Rust modules, or user-authored provider implementations.
- A live ACP Registry browser or automatic adoption of newly listed agents.
- Arbitrary custom commands, shell snippets, environment editors, or secret entry inside Editur.
- Shared conversations or session migration between providers.
- Provider-specific UI forks when the standard ACP rendering is sufficient.
- A general settings subsystem. Persist only the last selected provider.

## 3. Current architecture and coupling

Most of the current implementation is already the shared ACP client. The refactor should preserve it rather than replace it.

| Concern | Current state | Target boundary |
| --- | --- | --- |
| ACP connection and session loop | Standard ACP with one managed Cursor launch path | Shared unchanged; receives a selected provider and a prepared launch |
| Commands and normalized events | Provider-neutral except for a few Cursor labels and controls | Shared for all providers |
| Transcript and renderable state | Provider-neutral | Reset on provider switch; never mix providers in one transcript |
| Tool, permission, plan, usage, and content rendering | Mostly standard ACP | Shared; render optional capabilities only when advertised |
| Process containment and shutdown | Reusable mechanics with Cursor-specific error text and environment | Shared containment around a provider-owned launch specification |
| Provisioning | Secure but hardcoded to Cursor's manifest, host, paths, terms, and version check | Shared archive verification plus provider-owned policy |
| Vendor extensions | Cursor requests and notifications are registered in the controller | Provider-gated extension handlers outside the standard ACP path |
| Session removal history | Scoped by project but not provider | Scoped by provider and project |
| UI identity and copy | Cursor name, icon, authentication text, placeholders, and permission descriptions are embedded in the UI | Read from the selected provider descriptor |
| Release and installers | One embedded Cursor manifest and one provisioning command | One provider bundle; Cursor remains pre-provisioned and optional providers install lazily |

The existing normalized controller API, state reducer, fake agent, and buffer reconciliation are the valuable seam. Do not introduce a second controller per vendor.

## 4. Target architecture

```text
Agent UI and AgentState
        |
        | ProviderId + existing commands
        v
AgentController: standard ACP lifecycle
        |
        +---- standard ACP normalization
        |
        +---- selected provider extension handler
        |
        v
PreparedAgent: command, args, env, process guard
        ^
        |
provider catalog + provider-specific provisioning policy
```

### 4.1 Minimal provider model

Use a closed `ProviderId` enum and a static provider catalog. This is intentionally not a plugin trait or factory: the supported set is curated at release time, and a closed enum makes persistence, exhaustive handling, security review, and tests straightforward.

The conceptual types are:

| Type | Responsibility |
| --- | --- |
| `ProviderId` | Stable persisted key such as `cursor`, `codex`, or `claude` |
| `ProviderDescriptor` | Display name, compact icon, description, terms links, availability, install policy, and extension kind |
| `PreparedAgent` | Exact executable, arguments, environment, working directory behavior, and platform process guard |
| `ProviderBundle` | Versioned collection of pinned manifests embedded in one Editur build |
| `ProviderExtensions` | Closed enum such as `None` or `Cursor`; registers and normalizes only proven vendor additions |

Keep all provider matching in the provider and provisioning modules. The controller and UI should ask the catalog for metadata rather than match on Cursor, Codex, or Claude themselves.

Adding a third provider should normally require only:

1. A catalog entry and stable `ProviderId` variant.
2. A pinned release specification and launch preparation.
3. An extension handler only if standard ACP loses useful information.
4. Provider smoke-test results and user-facing terms copy.

It should not require edits to transcript rendering, standard command handling, buffer reconciliation, or the shared session loop.

### 4.2 Controller boundary

Change controller creation to receive a `ProviderId` with the project root and wake callback. Keep the existing test-only process launch seam.

The provider layer prepares the process before the standard connection loop starts. Preparation may provision a managed package, verify an existing package, or return an unsupported/setup error. Once prepared, the controller treats every provider as the same stdio ACP agent.

The controller continues to own:

- ACP initialization and stable protocol negotiation.
- Authentication requests advertised by the agent.
- Session creation, optional listing/loading, prompting, cancellation, and shutdown.
- Standard notifications, permission requests, bounded diagnostics, and normalized events.
- Exactly one active turn and bounded command/event queues.

The provider layer owns:

- Package and launch discovery.
- Provider-specific arguments and environment.
- Download-host and terms policy.
- Version validation.
- Optional namespaced requests, notifications, and metadata.
- Provider display identity used in generic error messages.

Make error construction provider-aware at the boundary, for example `cannot start Codex`, while keeping shared protocol errors generic where the provider name adds no value.

### 4.3 Extension isolation

Move Cursor's question, plan, todo, subagent, image-generation, and `run-everything` behavior behind `ProviderExtensions::Cursor`.

Register the known compile-time handlers required by the ACP SDK, but gate their parsing and UI events by the selected provider. Do not let another provider accidentally inherit Cursor semantics because it emits a similarly named command or field.

Start Codex and Claude on standard ACP only. Add a provider extension only after a fixture or smoke test proves that a useful feature cannot be represented through standard ACP. Unknown namespaced messages remain safely ignored.

### 4.4 Adapter selection

Use the canonical adapters published from the `agentclientprotocol` organization:

| Provider | Selected adapter | Why it wins |
| --- | --- | --- |
| Codex | [`@agentclientprotocol/codex-acp`](https://github.com/agentclientprotocol/codex-acp) | Official Registry entry; built on the Codex App Server; ships a compatible Codex dependency; covers authentication, session config, permissions, tools, plans, usage, review, images, subagents, and MCP; maintained where the former Zed adapter moved |
| Claude | [`@agentclientprotocol/claude-agent-acp`](https://github.com/agentclientprotocol/claude-agent-acp) | Official Registry entry; uses the official Claude Agent SDK; broad ACP coverage including permissions, terminals, TODOs, MCP, edit review, and an opt-in nested-subagent extension; maintained jointly in the ACP organization |

These are the canonical ecosystem adapters, not native ACP servers maintained solely by OpenAI or Anthropic. Keep that distinction in user documentation.

Do not ship a selectable adapter implementation underneath one provider name. One provider id maps to one audited adapter and one pinned version.

Community adapters remain comparison material, not automatic fallbacks:

- The former `zed-industries/codex-acp` is archived and directs new installs to the selected canonical Codex adapter.
- `cola-io/codex-acp` embeds the Codex Rust workspace and offers a native binary, but describes itself as subject to breaking changes and is not the current Registry entry.
- `rohan-patra/claude-agent-acp` adds useful Zed-specific review behavior, but does so by reverting writes after they occur and using a patched Claude Agent SDK. Editur already reconciles direct disk edits and should not take on that extra mutation and identity-patching risk.
- Multi-backend community bridges add another translation layer and a second compatibility surface without removing the need for provider-specific testing.

If a selected canonical adapter fails a release-blocking requirement, stop that provider's rollout. Re-evaluate alternatives in a documented spike against the same test matrix; never switch users to a fork silently.

## 5. Provider selection and switching

### 5.1 Placement

Provider selection belongs in the Agent header because it changes the process, account, session namespace, and available capabilities. It does not belong beside model and mode controls in the composer footer.

In the IDE layout, place a compact provider button at the far left of the right Agent header. Show the provider icon and name, followed by a disclosure arrow. Place the current session title after it and truncate the title before the existing New Session, History, and Close controls.

```text
[Codex v]  Fix failing release checks        [+] [History] [Close]
```

In the full Agent layout, make the existing identity row at the top of the session rail the same provider button. The main canvas title remains the session or project title.

```text
[Codex v]
[+ New session]

PROJECT
  Session one
  Session two
```

Do not render a one-item selector while Cursor is the only shipped provider. Add the control in the same slice that adds the first real second provider.

### 5.2 Provider menu

Each row shows:

- Provider icon and display name.
- A short description.
- Current selection.
- `Installs on first use` or an unavailable-platform reason when relevant.

Use text in addition to icons and color. The menu must be keyboard reachable, keep existing minimum target sizes, close with Escape or outside click, and expose provider names through widget accessibility labels.

Do not place model names in this menu. Models, session modes, sandbox controls, and ordinary ACP config options stay in the composer footer and update after the new provider creates or loads a session.

### 5.3 Safe switch lifecycle

Provider switching is allowed only while no turn, permission request, or interaction request is active. Otherwise disable the rows and explain that the user must stop or answer the current request first.

When the current transcript is non-empty, confirm the switch with concise copy:

> Switch to Codex? This closes the Cursor connection and clears the visible transcript. Provider history is not deleted, but agents without session loading cannot restore it.

After confirmation:

1. Preserve the unsent composer draft but never send it automatically.
2. Close provider and session menus.
3. Move the current controller to a background shutdown transition.
4. Wait for shutdown completion before launching the next provider, keeping the UI responsive in a `Switching to ...` state.
5. Clear connection, session, transcript, usage, permission, and provider-advertised config state.
6. Set and persist the new `ProviderId`.
7. Provision or verify the selected provider, connect, and load its newest project session only when it advertises session history; otherwise create a session.

If the new provider fails to install, authenticate, or connect, keep it selected so Retry is meaningful. The user can switch back through the same menu. Never revive the old process automatically.

## 6. Capability and authentication behavior

The shared UI is capability-driven:

- Show History only when session listing and loading are both advertised.
- Show mode and configuration controls only from the active session response.
- Treat model selection as an ordinary advertised configuration option.
- Show standard permission choices exactly as supplied by the provider.
- Keep `Allow all` hidden unless the selected provider's tested adapter explicitly supports the current Cursor behavior or exposes an equivalent standard option.
- Do not infer a provider's capabilities from its name.

Preserve authentication method kind as well as id, name, and description. Browser/agent-owned authentication can use the current Connect flow. A terminal authentication method needs a real embedded terminal or a separate supported setup flow; do not render it as a browser button. Providers that expose only an unsupported auth kind remain unavailable with a clear reason.

Editur must not collect or persist provider API keys. Environment-based credentials remain owned by the provider process and the user's environment.

All authentication and connection copy uses provider metadata: `Connect Codex`, `Connecting to Claude...`, and `Codex is offline`. Generic titlebar actions remain `Open Agent` and `Close Agent`.

## 7. Provisioning and release design

### 7.1 Provider bundle

Replace the single embedded sidecar manifest with a versioned provider bundle containing zero or one pinned package manifest per supported provider for the current target.

Keep the existing update asset name for compatibility. A format-v1 single Cursor manifest is read as a one-provider bundle. A format-v2 bundle carries provider ids and manifests. Support format v1 for at least one release cycle.

The generic provisioner keeps the existing safe mechanics:

- Bounded HTTPS download.
- Archive checksum and extraction limits.
- Path traversal protection.
- Exact file list, kind, size, and checksum validation.
- Staged activation and rollback.
- Fast verification receipts.
- Current and one prior known-good version.
- Per-provider process containment and descendant teardown.

Move storage and locks under a provider namespace. Cursor's existing managed installation already uses a Cursor namespace and should remain valid without redownloading.

### 7.2 Provider-owned policy

The generic manifest parser must not accept arbitrary hosts or commands. After parsing `ProviderId`, validate the manifest against compiled provider policy:

- Allowed distribution kinds and exact HTTPS hosts.
- License and terms requirements.
- Supported operating systems and architectures.
- Command and entrypoint rules.
- Required launch arguments and environment restrictions.
- Version probe and expected output.
- Auto-update disablement or immutability requirements.

The release generator may consume ACP Registry metadata, but CI pins the selected version and produces Editur's attested manifest. The application never fetches the mutable Registry index at startup or provider-selection time.

### 7.3 Binary versus package-runner distributions

The current official Registry entries use a platform binary for Cursor and exact `npx` packages for Codex and Claude. Treat that as a release spike, not as permission to execute `npx` on an end user's machine.

For each non-binary provider, decide before integration whether Editur can:

1. Use a provider-published standalone artifact with a stable license and checksum, or
2. Download an exact package and run it from Editur's private version directory with a pinned, supportable runtime.

Do not depend on a globally installed Node.js, execute a mutable package tag, invoke a remote install script, or bundle a general Node runtime during the Cursor-only refactor. If neither pinned option meets size, licensing, update, and platform requirements, leave that provider unavailable rather than weakening the existing supply-chain guarantees.

### 7.4 Install and update behavior

- Bootstrap installation continues to provision Cursor so existing behavior does not change.
- Optional providers provision lazily after the user selects them and accepts any first-use terms notice.
- Self-update verifies the new provider bundle and keeps the current Cursor provisioning guarantee. Other providers repair lazily on next use.
- The internal provision command accepts an optional provider id; no id continues to mean Cursor for installer compatibility.
- Removing a provider from a later release stops offering it but does not recursively delete its existing data.

## 8. Persistence and migration

Persist the last selected `ProviderId` in one small atomic application-data preference. Cursor is the default when the preference is absent, invalid, or names a provider unavailable in the current build. Per-project preferences are deferred.

Namespace hidden-session history by both provider and project. Migrate the existing unnamespaced history directory to Cursor's namespace once, only when the destination does not already exist. Session ids from different providers must never share a hidden-session set.

Do not persist transcripts, permission requests, connection state, or provider capabilities in this stage. Those values are only valid for the live connection or provider-owned session history.

## 9. TDD and verification strategy

Follow the existing fake-process approach. Add one failing behavioral test for each non-trivial branch, then the smallest implementation that passes. Do not duplicate the entire controller suite for every provider.

### 9.1 Focused automated checks

Provider foundation:

- Cursor is selected by default when no preference exists.
- Unknown or unavailable persisted ids fall back to Cursor.
- A selected provider produces the expected prepared command, arguments, environment, and display name.
- Session-removal history is isolated by provider and project, including the Cursor migration.
- Cursor extension requests are handled for Cursor and ignored for a non-Cursor fake provider.
- Generic ACP text, tools, permissions, cancellation, and shutdown pass unchanged for two fake provider ids.
- Switching waits for old-process shutdown before starting the next process.
- Switching is rejected during a turn, permission, or interaction.
- A failed new provider remains selected and can retry or switch back.

Provisioning:

- Format-v1 Cursor manifests load as a one-provider bundle.
- Format-v2 bundles reject duplicate or unknown provider ids.
- Each provider rejects an unapproved host, command, platform, checksum, and version response.
- Provider locks, active markers, versions, cache, and verification receipts cannot collide.
- An installed Cursor version remains usable after the bundle migration.
- Lazy optional-provider provisioning does not run during normal startup.

State and UI:

- Provider switch clears provider-bound state while preserving the unsent draft.
- The provider selector occupies the header identity position in both layouts.
- The menu is absent with one provider and lists exact catalog entries with two.
- The provider selector is distinct from advertised model and mode selectors.
- Cursor-specific copy disappears when another provider is selected.
- Keyboard focus, accessibility names, menu closing, and disabled-switch explanations work.

Keep layout checks focused on Editur's geometry and interaction decisions. Do not test egui internals, vendor ACP implementations, or network availability.

### 9.2 Manual provider smoke matrix

For every supported native target and provider:

1. Clean install or first-use provisioning.
2. Authentication with no credentials passing through Editur.
3. Standard ACP initialization and capability capture.
4. New session and, when advertised, list/load history.
5. Streaming response and same-session follow-up.
6. Tool activity, file edit, exact permission allow/reject, and cancellation.
7. Buffer reconciliation and project refresh.
8. Provider switch, old-process teardown, switch back, and session isolation.
9. Missing/corrupt package repair.
10. Application exit with no descendant process.

Record protocol version, advertised capabilities, package size, installed size, connection time, and unsupported features. Networked provider tests remain manual and must not run in CI with real accounts or paid tokens.

### 9.3 Performance checks

- Normal editor startup with Agent unopened remains within the existing baseline.
- Merely rendering the generic Agent toggle does not inspect provider installations.
- Only the selected provider is provisioned, launched, polled, or retained.
- Switching and shutdown never block the UI thread.
- Provider bundle parsing and package verification occur on the existing background controller path.
- Check this project's build-directory size before and after dependency or runtime changes; remove only this project's incremental artifacts when necessary.

## 10. Implementation phases

### Phase 0: Freeze Cursor behavior

- Add focused regression tests around Cursor launch selection, extension gating, session-history location, status copy, and shutdown.
- Record the current Cursor smoke results as the compatibility baseline.
- Make no visible UI change.

Exit condition: the tests describe the behavior that must survive extraction.

### Phase 1: Extract the provider boundary

- Add `ProviderId`, the static catalog, `PreparedAgent`, and the closed extension enum.
- Pass the selected provider into controller creation and managed launch preparation.
- Move Cursor launch environment, version verification, labels, and extensions behind the provider boundary.
- Generalize process-containment error text and keep one controller.
- Namespace provider session history with the tested Cursor migration.
- Keep the production catalog Cursor-only.

Exit condition: Cursor passes the existing controller, state, UI, provisioning, and smoke tests through the new boundary with no user-visible behavior change.

### Phase 2: Generalize release metadata

- Add format-v2 provider bundles and format-v1 compatibility.
- Separate generic archive safety from provider host, command, terms, and version policy.
- Update manifest generation, update provisioning, CI artifacts, and the internal provision command.
- Keep installers provisioning Cursor only.

Exit condition: a Cursor-only release uses the provider bundle, upgrades an existing installation without redownloading valid files, and rolls back safely.

### Phase 3: Add Codex as the proof provider

Codex is the recommended proof provider because the selected canonical adapter is listed in the official Registry under Apache-2.0 and exercises a non-binary distribution path. This recommendation remains conditional on the distribution spike.

- Pin and test one release of `@agentclientprotocol/codex-acp` with its bundled compatible Codex dependency.
- Resolve the package/runtime, update, authentication, and terms gates before UI work.
- Start with standard ACP and no Codex-specific extension handler.
- Add Codex to the production catalog and provider bundle.
- Add the provider selector in both Agent layouts.
- Add switching, persistence, lazy provisioning, generic copy, and provider-scoped sessions.

Exit condition: Cursor and Codex can be selected, authenticated, used, stopped, switched, restored, updated, and removed from view without process or session leakage.

### Phase 4: Add Claude through the same path

- Pin and test one release of `@agentclientprotocol/claude-agent-acp` with its declared official Claude Agent SDK dependency.
- Resolve its package/runtime, proprietary license, authentication, auto-update, and platform gates.
- Use standard ACP rendering first.
- Add only proven Claude metadata or nested-transcript support behind a Claude extension entry and explicit client capability.
- Re-run the same provider smoke matrix without adding a parallel controller or Claude-only view.

Exit condition: Claude requires only provider catalog, release policy, fixtures, and justified extensions; shared UI and controller logic remain unchanged.

### Phase 5: Release hardening

- Update user documentation, installer disclosure, performance records, and release smoke instructions.
- Verify native macOS, Linux, and Windows teardown and first-use flows.
- Recheck every provider's registry entry, distribution terms, checksums, auth behavior, and auto-update behavior before promotion.
- Keep a release rollback path that can remove an optional provider from the catalog while leaving Cursor functional.

Exit condition: a stable release can disable one optional provider without disabling Agent mode or corrupting another provider's installation or sessions.

## 11. Acceptance criteria

- [ ] Cursor remains the default and its current automated and manual behavior passes unchanged.
- [ ] No provider work occurs before Agent is opened.
- [ ] One static catalog is the only source of provider identity and availability used by the UI.
- [ ] The standard ACP controller, normalized events, transcript state, and buffer reconciliation contain no Cursor, Codex, or Claude branches.
- [ ] Vendor extensions are selected explicitly and cannot leak across providers.
- [ ] The provider selector appears in the Agent header/identity row only when at least two providers ship.
- [ ] Provider and model selection are visually and behaviorally distinct.
- [ ] Switching is impossible during an active or blocking request, shuts down the old process first, and clears provider-bound state.
- [ ] Sessions, hidden history, diagnostics, configuration, and managed files are provider-scoped.
- [ ] Optional providers are pinned, verified, and provisioned lazily without global CLI or `PATH` changes.
- [ ] A third provider can be added without changing shared transcript rendering, the standard controller loop, or buffer reconciliation.
- [ ] Automated tests require no provider account, network access, or paid usage.
- [ ] Unsupported distribution, authentication, protocol, or licensing behavior blocks that provider rather than weakening safety guarantees.

## 12. Risks and stop conditions

| Risk | Default response | Revisit when |
| --- | --- | --- |
| An ACP provider requires a global runtime or mutable package execution | Do not ship that provider | A pinned private runtime/artifact meets size and support targets |
| Provider auth is terminal-only | Mark setup unsupported | Editur has a tested terminal-auth surface |
| A provider lacks session loading | Create new sessions and warn before switching away | The provider advertises stable list/load support |
| Vendor extensions diverge | Keep standard fallback and ignore unknown metadata | A tested extension materially improves the UI |
| Optional providers inflate install size | Provision only the selected optional provider | Measurements justify preinstallation |
| A provider self-updates managed files | Block release | A supported disable/update contract exists |
| Terms or Registry distribution changes | Remove the provider from the catalog | A documented, permitted distribution path returns |
| The provider abstraction starts accumulating one-off methods | Stop and keep the behavior in its provider module | Two providers prove the method is truly shared |

## 13. Reference snapshot

Checked 2026-08-08:

- [ACP Registry format](https://github.com/agentclientprotocol/registry/blob/main/FORMAT.md)
- [Cursor Registry entry](https://github.com/agentclientprotocol/registry/blob/main/cursor/agent.json)
- [Codex Registry entry](https://github.com/agentclientprotocol/registry/blob/main/codex-acp/agent.json)
- [Codex ACP adapter](https://github.com/agentclientprotocol/codex-acp)
- [Archived Zed Codex adapter and migration notice](https://github.com/zed-industries/codex-acp)
- [Community native Codex adapter](https://github.com/cola-io/codex-acp)
- [Claude Registry entry](https://github.com/agentclientprotocol/registry/blob/main/claude-acp/agent.json)
- [Claude ACP adapter](https://github.com/agentclientprotocol/claude-agent-acp)
- [Community Claude edit-review fork](https://github.com/rohan-patra/claude-agent-acp)
- [ACP Registry authentication requirements](https://github.com/agentclientprotocol/registry/blob/main/AUTHENTICATION.md)

Registry versions and distribution shapes are release inputs, not runtime assumptions. Pin and re-verify them for every Editur release.
